use super::*;
use crate::app::TelegramCommand;

impl Settings {
    pub(super) fn new_telegram_token(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let token = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Вставьте токен")
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
        let label = bot.proxy_port.map_or_else(
            || "Системные настройки macOS".to_owned(),
            |port| format!("SOCKS5 · 127.0.0.1:{port}"),
        );
        let mut section = col().child(Self::setting(
            p,
            "Подключение",
            &label,
            button("telegram-network", "Изменить", p)
                .disabled(bot.enabled && !bot.route_ready)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.telegram_network_open = !this.telegram_network_open;
                    let port = this.snapshot.telegram.proxy_port.map_or(10808, |p| p.get());
                    this.telegram_port.update(cx, |input, cx| {
                        input.set_value(port.to_string(), window, cx)
                    });
                    cx.notify();
                })),
        ));
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
                    .child(text(
                        "Порт локального SOCKS5-прокси на 127.0.0.1",
                        12.,
                        p.muted,
                    ))
                    .child(
                        row()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(78.))
                                    .child(Input::new(&self.telegram_port).small()),
                            )
                            .child(
                                primary_button("telegram-use-proxy", "Через прокси", p)
                                    .disabled(!valid)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.apply_telegram_proxy(cx)),
                                    ),
                            )
                            .child(
                                button("telegram-system-network", "Системная сеть", p).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.send(Command::Telegram(TelegramCommand::SetRoute(
                                            None,
                                        )));
                                        this.telegram_network_open = false;
                                        cx.notify();
                                    }),
                                ),
                            ),
                    ),
            );
        }
        section
    }

    pub(super) fn telegram(&self, p: Palette, cx: &Context<Self>) -> Div {
        col()
            .gap_2()
            .child(text(
                "Перешлите боту голосовое, аудио или видео до 20 МБ. Voai расшифрует запись на Mac и пришлёт текст в тот же чат.",
                13.,
                p.muted,
            ))
            .child(self.telegram_network(p,cx))
            .child(if self.snapshot.telegram.enabled {
                self.telegram_connection(p, cx)
            } else {
                self.telegram_setup(p, cx)
            })
            .child(text(
                "Бот на связи, пока Voai запущен и Mac подключён к интернету.",
                12.,
                p.muted,
            ).mt_2())
    }

    fn telegram_setup(&self, p: Palette, cx: &Context<Self>) -> Div {
        let status = &self.snapshot.telegram.status;
        col()
            .mt_2()
            .gap_2()
            .child(
                row()
                    .justify_between()
                    .gap_2()
                    .child(label("Токен бота", p.ink))
                    .child(
                        Button::new("telegram-botfather")
                            .ghost()
                            .small()
                            .label("Открыть @BotFather")
                            .text_color(p.accent)
                            .on_click(|_, _, cx| cx.open_url("https://t.me/BotFather")),
                    ),
            )
            .child(text(
                "Создайте бота командой /newbot в @BotFather и вставьте полученный токен.",
                12.,
                p.muted,
            ))
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
                        primary_button("telegram-connect", "Подключить", p)
                            .disabled(self.telegram_token.read(cx).value().trim().is_empty())
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.connect_telegram(window, cx)
                                }),
                            ),
                    ),
            )
            .when(!status.is_empty(), |d| {
                d.child(text(status.clone(), 12., p.error))
            })
    }

    fn telegram_connection(&self, p: Palette, cx: &Context<Self>) -> Div {
        let bot = &self.snapshot.telegram;
        let name = bot
            .username
            .as_ref()
            .map_or_else(|| "Бот".to_owned(), |name| format!("@{name}"));
        let mut page = col().gap_2().child(Self::setting_row(
            p,
            &name,
            Some(text(
                bot.status.clone(),
                12.,
                if bot.running { p.muted } else { p.error },
            )),
            button("telegram-disconnect", "Отключить", p)
                .tooltip("Остановить бота и удалить его токен с этого Mac")
                .on_click(cx.listener(|this, _, _, _| {
                    this.send(Command::Telegram(TelegramCommand::Disconnect))
                })),
        ));
        if bot.awaiting_confirmation {
            page = page.child(
                row()
                    .gap_2()
                    .child(
                        primary_button("telegram-confirm", "Уже в чате", p).on_click(cx.listener(
                            |this, _, _, _| {
                                this.send(Command::Telegram(TelegramCommand::ConfirmDelivery))
                            },
                        )),
                    )
                    .child(button("telegram-resend", "Отправить ещё раз", p).on_click(
                        cx.listener(|this, _, _, _| {
                            this.send(Command::Telegram(TelegramCommand::ResendPart))
                        }),
                    )),
            );
        } else if !bot.running {
            page = page.child(row().child(
                primary_button("telegram-retry", "Повторить", p).on_click(cx.listener(
                    |this, _, _, _| this.send(Command::Telegram(TelegramCommand::Retry)),
                )),
            ));
        } else if bot.owner.is_none() && bot.pairing_url.is_some() {
            let pair = primary_button("telegram-pair", "Привязать аккаунт", p).on_click(
                cx.listener(|this, _, _, cx| {
                    if let Some(url) = &this.engine.borrow().snapshot().telegram.pairing_url {
                        cx.open_url(url);
                    }
                }),
            );
            page = page
                .child(text("Откройте ссылку под своим аккаунтом Telegram и нажмите «Старт». Бот будет принимать записи только от этого аккаунта.", 13., p.ink))
                .child(row().child(pair));
        }
        if let Some(owner) = bot.owner
            && let Some(name) = &bot.username
        {
            let url = format!("https://t.me/{name}");
            page = page.child(Self::setting(
                p,
                "Разрешённый отправитель",
                &format!("Telegram ID {owner}"),
                button("telegram-chat", "Открыть чат", p)
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            ));
        }
        page.child(Self::setting(
            p,
            "Модели",
            "Речь и обработка текста из раздела «Расшифровка»",
            button("telegram-models", "Настроить", p).on_click(cx.listener(
                |this, _, window, cx| {
                    this.files_ui.configure();
                    this.navigate(Page::Files, window, cx);
                },
            )),
        ))
    }
}
