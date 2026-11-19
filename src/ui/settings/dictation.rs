use super::*;

impl Settings {
    fn speech_model(&self, p: Palette, cx: &Context<Self>) -> Div {
        let models = self.snapshot.models.iter().filter(|m| !m.text_model);
        let selected = models
            .clone()
            .find(|m| matches!(m.state, ModelState::Selected));
        let downloading = models.clone().find_map(|m| match &m.state {
            ModelState::Downloading { received } => Some(format!(
                "{} · загружено {} из {}",
                m.name,
                bytes(*received),
                bytes(m.bytes)
            )),
            _ => None,
        });
        let (detail, action) = if let Some(m) = selected {
            (m.name.clone(), button("speech-model", "Сменить", p))
        } else if let Some(progress) = downloading {
            (progress, button("speech-model", "Открыть модели", p))
        } else {
            (
                if models
                    .clone()
                    .any(|m| matches!(m.state, ModelState::Installed))
                {
                    "Выберите одну из установленных моделей."
                } else {
                    "Скачайте из каталога или подключите файл Whisper."
                }
                .into(),
                primary_button("speech-model", "Выбрать модель", p),
            )
        };
        Self::setting(
            p,
            "Речевая модель",
            &detail,
            action.on_click(
                cx.listener(|this, _, window, cx| this.show_dictation_setup(true, window, cx)),
            ),
        )
    }

    fn permission(&self, allowed: bool, mic: bool, p: Palette, cx: &Context<Self>) -> AnyElement {
        if allowed {
            row()
                .gap_1()
                .child(icon("check", p.accent, 15.))
                .child(text("Разрешено", 12., p.accent))
                .into_any_element()
        } else {
            button(
                if mic { "grant-mic" } else { "grant-access" },
                "Разрешить",
                p,
            )
            .on_click(cx.listener(move |this, _, _, _| {
                this.send(if mic {
                    Command::RequestMicrophone
                } else {
                    Command::RequestAccess
                })
            }))
            .into_any_element()
        }
    }

    pub(super) fn dictation(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let s = &self.snapshot;
        let mut devices = vec![("Системный микрофон".to_string(), String::new())];
        devices.extend(
            s.microphones
                .iter()
                .cloned()
                .map(|name| (name.clone(), name)),
        );
        let languages: Vec<_> = [
            ("Автоматически", "auto"),
            ("Русский", "ru"),
            ("English", "en"),
        ]
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .into();
        let language = languages
            .iter()
            .find(|(_, v)| v == &s.language)
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| s.language.clone());
        col()
            .child(
                row()
                    .w_full()
                    .justify_between()
                    .gap_3()
                    .p(px(12.))
                    .mb(px(6.))
                    .rounded(px(8.))
                    .bg(p.side)
                    .border_1()
                    .border_color(p.rule)
                    .child(
                        row()
                            .gap(px(11.))
                            .child(
                                row()
                                    .size(px(34.))
                                    .rounded(px(8.))
                                    .justify_center()
                                    .bg(p.soft)
                                    .child(icon("mic", p.accent, 18.)),
                            )
                            .child(
                                col()
                                    .gap(px(2.))
                                    .child(label("Клавиши диктовки", p.ink))
                                    .child(text(
                                        self.shortcut_error.unwrap_or("Удерживайте, пока говорите"),
                                        12.,
                                        p.muted,
                                    )),
                            ),
                    )
                    .child(
                        Button::new("shortcut")
                            .small()
                            .rounded(px(6.))
                            .bg(p.surface)
                            .border_color(p.rule)
                            .child(if self.capture {
                                text("Нажмите сочетание…", 12., p.ink)
                            } else {
                                row().gap(px(4.)).children(
                                    shortcut_label(&s.shortcut).split_whitespace().map(|key| {
                                        text(key.to_owned(), 12., p.ink)
                                            .px(px(4.))
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(p.rule)
                                            .bg(p.side)
                                    }),
                                )
                            })
                            .disabled(s.phase != crate::app::Phase::Idle)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.send(Command::CaptureShortcut(true));
                                this.capture = true;
                                window.focus(&this.focus);
                                cx.notify();
                            })),
                    ),
            )
            .child(self.speech_model(p, cx))
            .child(Self::setting_row(
                p,
                "Микрофон",
                Some(
                    row()
                        .gap_2()
                        .child(text("Входной уровень", 12., p.muted))
                        .child(row().gap(px(2.)).children((0..7).map(|i| {
                            let lit = s.phase == crate::app::Phase::Listening
                                && s.level.clamp(0., 1.).sqrt() * 7. > i as f32;
                            div()
                                .w(px(3.))
                                .h(px(4. + i as f32))
                                .rounded_full()
                                .bg(if lit { p.accent } else { p.rule })
                        }))),
                ),
                self.picker(
                    "microphone",
                    s.microphone
                        .clone()
                        .unwrap_or_else(|| "Системный микрофон".into()),
                    devices,
                    s.microphone.clone().unwrap_or_default(),
                    |id| Command::SetMicrophone((!id.is_empty()).then_some(id)),
                    p,
                ),
            ))
            .child(Self::setting(
                p,
                "Язык речи",
                "",
                self.picker(
                    "language",
                    language,
                    languages,
                    s.language.clone(),
                    Command::SetLanguage,
                    p,
                ),
            ))
            .child(group_title("Разрешения", p))
            .child(Self::setting(
                p,
                "Микрофон",
                "Для записи вашего голоса",
                self.permission(s.mic_allowed, true, p, cx),
            ))
            .child(Self::setting(
                p,
                "Универсальный доступ",
                "Для ввода текста у курсора",
                self.permission(s.access_allowed, false, p, cx),
            ))
            .child(group_title("Пробная диктовка", p))
            .child(Input::new(&self.test).h(px(58.)).w_full())
    }
}
