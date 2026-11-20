use super::*;
use crate::app::TelegramCommand;
use voai_l10n::{tf, tr};

impl Settings {
    pub(super) fn new_telegram_token(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let token = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(tr("ui.telegram.paste_token"))
        });
        cx.subscribe_in(&token, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.connect_telegram(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        })
        .detach();
        token
    }

    fn connect_telegram(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Telegram || self.snapshot.telegram.enabled {
            return;
        }
        let draft = self.telegram_token.read(cx).value();
        if draft.trim().is_empty() {
            return;
        }
        self.send(Command::Telegram(TelegramCommand::Connect(
            draft.trim().to_owned(),
        )));
        // A fresh field discards undo history along with the visible token.
        self.telegram_token = Self::new_telegram_token(window, cx);
        window.focus(&self.focus);
        cx.notify();
    }

    pub(super) fn apply_telegram_proxy(&mut self, cx: &mut Context<Self>) {
        if let Ok(port) = self
            .telegram_port
            .read(cx)
            .value()
            .trim()
            .parse::<std::num::NonZeroU16>()
        {
            self.send(Command::Telegram(TelegramCommand::SetRoute(Some(port))));
            self.telegram_network_open = false;
            cx.notify();
        }
    }

    fn telegram_network(&self, p: Palette, cx: &Context<Self>) -> Div {
        let bot = &self.snapshot.telegram;
        let title = match (self.telegram_network_open, bot.proxy_port) {
            (true, _) => tr("ui.telegram.hide_network").to_owned(),
            (false, Some(port)) => tf!("ui.telegram.proxy_port", port = port),
            (false, None) => tr("ui.telegram.network").to_owned(),
        };
        let mut section = col().child(
            row().child(
                Button::new("telegram-network")
                    .ghost()
                    .small()
                    .label(title)
                    .disabled(bot.enabled && !bot.route_ready)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.telegram_network_open = !this.telegram_network_open;
                        if this.telegram_network_open {
                            let port = this.snapshot.telegram.proxy_port;
                            this.telegram_port.update(cx, |input, cx| {
                                input.set_value(
                                    port.map(|p| p.to_string()).unwrap_or_default(),
                                    window,
                                    cx,
                                )
                            });
                        }
                        cx.notify();
                    })),
            ),
        );
        if self.telegram_network_open {
            let valid = self
                .telegram_port
                .read(cx)
                .value()
                .trim()
                .parse::<std::num::NonZeroU16>()
                .is_ok();
            section = section.child(
                col()
                    .gap_2()
                    .child(text(tr("ui.telegram.network_detail"), 12., p.muted))
                    .child(
                        row()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(78.))
                                    .child(Input::new(&self.telegram_port).small()),
                            )
                            .child(
                                primary_button(
                                    "telegram-use-proxy",
                                    tr("ui.telegram.use_proxy"),
                                    p,
                                )
                                .disabled(!valid)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.apply_telegram_proxy(cx)),
                                ),
                            )
                            .child(
                                button("telegram-system-network", tr("ui.telegram.use_system"), p)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.send(Command::Telegram(TelegramCommand::SetRoute(
                                            None,
                                        )));
                                        this.telegram_network_open = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        section
    }

    pub(super) fn telegram(&self, p: Palette, cx: &Context<Self>) -> Div {
        col()
            .gap_2()
            .child(text(tr("ui.telegram.intro"), 13., p.muted))
            .child(if self.snapshot.telegram.enabled {
                self.telegram_connection(p, cx)
            } else {
                self.telegram_setup(p, cx)
            })
            .child(text(tr("ui.telegram.online_hint"), 12., p.muted).mt_2())
            .child(self.telegram_network(p, cx))
    }

    fn telegram_setup(&self, p: Palette, cx: &Context<Self>) -> Div {
        let status = self.snapshot.telegram.status.to_string();
        col()
            .mt_2()
            .gap_2()
            .child(
                row()
                    .justify_between()
                    .gap_2()
                    .child(label(tr("ui.telegram.bot_token"), p.ink))
                    .child(
                        Button::new("telegram-botfather")
                            .ghost()
                            .small()
                            .label(tr("ui.telegram.open_botfather"))
                            .text_color(p.accent)
                            .on_click(|_, _, cx| cx.open_url("https://t.me/BotFather")),
                    ),
            )
            .child(text(tr("ui.telegram.create_bot"), 12., p.muted))
            .child(
                row()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.telegram_token).small()),
                    )
                    .child(
                        primary_button("telegram-connect", tr("ui.action.connect"), p)
                            .disabled(self.telegram_token.read(cx).value().trim().is_empty())
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.connect_telegram(window, cx)
                                }),
                            ),
                    ),
            )
            .when(!status.is_empty(), |d| d.child(text(status, 12., p.error)))
    }

    fn telegram_connection(&self, p: Palette, cx: &Context<Self>) -> Div {
        let bot = &self.snapshot.telegram;
        let name = bot.username.as_ref().map_or_else(
            || tr("ui.telegram.bot").to_owned(),
            |name| format!("@{name}"),
        );
        let mut page = col().gap_2().child(Self::setting_row(
            p,
            &name,
            Some(text(
                bot.status.to_string(),
                12.,
                if bot.running { p.muted } else { p.error },
            )),
            button("telegram-disconnect", tr("ui.action.disconnect"), p)
                .tooltip(tr("ui.telegram.disconnect_hint"))
                .on_click(cx.listener(|this, _, _, _| {
                    this.send(Command::Telegram(TelegramCommand::Disconnect))
                })),
        ));
        if bot.awaiting_confirmation {
            page = page.child(
                row()
                    .gap_2()
                    .child(
                        primary_button("telegram-confirm", tr("ui.telegram.confirm_delivery"), p)
                            .on_click(cx.listener(|this, _, _, _| {
                                this.send(Command::Telegram(TelegramCommand::ConfirmDelivery))
                            })),
                    )
                    .child(
                        button("telegram-resend", tr("ui.telegram.resend"), p).on_click(
                            cx.listener(|this, _, _, _| {
                                this.send(Command::Telegram(TelegramCommand::ResendPart))
                            }),
                        ),
                    ),
            );
        } else if !bot.running {
            page = page.child(row().child(
                primary_button("telegram-retry", tr("ui.action.retry"), p).on_click(cx.listener(
                    |this, _, _, _| this.send(Command::Telegram(TelegramCommand::Retry)),
                )),
            ));
        } else if bot.owner.is_none() && bot.pairing_url.is_some() {
            let pair = primary_button("telegram-pair", tr("ui.telegram.link_account"), p).on_click(
                cx.listener(|this, _, _, cx| {
                    if let Some(url) = &this.engine.borrow().snapshot().telegram.pairing_url {
                        cx.open_url(url);
                    }
                }),
            );
            page = page
                .child(text(tr("ui.telegram.pair_help"), 13., p.ink))
                .child(row().child(pair));
        }
        if let Some(owner) = bot.owner
            && let Some(name) = &bot.username
        {
            let url = format!("https://t.me/{name}");
            page = page.child(Self::setting(
                p,
                tr("ui.telegram.sender"),
                &format!("Telegram ID {owner}"),
                button("telegram-chat", tr("ui.telegram.open_chat"), p)
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            ));
        }
        page.child(Self::setting(
            p,
            tr("ui.page.models"),
            tr("ui.telegram.model_source"),
            button("telegram-models", tr("ui.action.configure"), p).on_click(cx.listener(
                |this, _, window, cx| {
                    this.files_ui.configure();
                    this.navigate(Page::Files, window, cx);
                },
            )),
        ))
    }
}
