use super::*;
use voai_l10n::InterfaceLanguage;
use voai_l10n::{tf, tr};

impl Settings {
    pub(in crate::ui) fn test_widget_anchor(&self) -> Option<objc2_core_foundation::CGRect> {
        self.test_field_frame.get().filter(|_| self.test_ready)
    }

    fn speech_model(&self, p: Palette, cx: &Context<Self>) -> Div {
        let models = self.snapshot.models.iter().filter(|m| !m.text_model);
        let selected = models
            .clone()
            .find(|m| matches!(m.state, ModelState::Selected));
        let downloading = models.clone().find_map(|m| match &m.state {
            ModelState::Downloading { received } => Some(tf!(
                "ui.model.download_named",
                model = &m.name,
                received = bytes(*received),
                total = bytes(m.bytes)
            )),
            _ => None,
        });
        let (detail, action) = if let Some(m) = selected {
            (
                m.name.clone(),
                button("speech-model", tr("ui.action.change"), p),
            )
        } else if let Some(progress) = downloading {
            (progress, button("speech-model", tr("ui.model.open"), p))
        } else {
            (
                if models
                    .clone()
                    .any(|m| matches!(m.state, ModelState::Installed))
                {
                    tr("ui.model.pick_installed")
                } else {
                    tr("ui.model.first_download")
                }
                .into(),
                primary_button("speech-model", tr("ui.model.choose"), p),
            )
        };
        Self::setting(
            p,
            tr("ui.dictation.model"),
            &detail,
            action.on_click(
                cx.listener(|this, _, window, cx| this.show_dictation_setup(true, window, cx)),
            ),
        )
    }

    pub(super) fn permission(
        &self,
        allowed: bool,
        mic: bool,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        if allowed {
            row()
                .gap_1()
                .child(icon("check", p.accent, 15.))
                .child(text(tr("ui.permission.allowed"), 12., p.accent))
                .into_any_element()
        } else {
            button(
                if mic { "grant-mic" } else { "grant-access" },
                tr("ui.permission.allow"),
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
        let mut devices = vec![(tr("ui.microphone.default").to_string(), String::new())];
        devices.extend(
            s.microphones
                .iter()
                .cloned()
                .map(|name| (name.clone(), name)),
        );
        let languages: Vec<_> = [
            (tr("ui.speech.automatic"), "auto"),
            (tr("ui.lang.russian"), "ru"),
            (tr("ui.language.english"), "en"),
        ]
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .into();
        let language = languages
            .iter()
            .find(|(_, v)| v == &s.language)
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| s.language.clone());
        let interface_name = |language| match language {
            InterfaceLanguage::System => tr("ui.language.system"),
            InterfaceLanguage::English => tr("ui.language.english"),
            InterfaceLanguage::Russian => tr("ui.lang.russian"),
        };
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
                                    .child(label(tr("ui.dictation.shortcut"), p.ink))
                                    .child(text(
                                        self.shortcut_error.map(tr).unwrap_or(if self.capture {
                                            tr("ui.shortcut.modifiers")
                                        } else {
                                            tr("ui.shortcut.hold")
                                        }),
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
                                text(tr("ui.shortcut.press"), 12., p.ink)
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
                                this.capture = this
                                    .engine
                                    .borrow_mut()
                                    .try_dispatch(Command::CaptureShortcut(true));
                                window.focus(&this.focus);
                                cx.notify();
                            })),
                    ),
            )
            .child(Self::setting(p, tr("ui.dictation.widget"), "", {
                let compact = s.compact_widget;
                Button::new("widget-style")
                    .small()
                    .rounded(px(6.))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.rule)
                    .child(
                        row()
                            .gap_2()
                            .child(text(
                                if compact {
                                    tr("ui.widget.compact")
                                } else {
                                    tr("ui.widget.standard")
                                },
                                12.,
                                p.ink,
                            ))
                            .child(icon("chevron", p.muted, 13.)),
                    )
                    .dropdown_menu({
                        let engine = self.engine.clone();
                        move |mut menu, _, _| {
                            for (value, label) in [
                                (false, tr("ui.widget.standard")),
                                (true, tr("ui.widget.compact")),
                            ] {
                                let engine = engine.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label)
                                        .checked(value == compact)
                                        .on_click(move |_, _, _| {
                                            engine
                                                .borrow_mut()
                                                .dispatch(Command::SetCompactWidget(value))
                                        }),
                                );
                            }
                            menu
                        }
                    })
            }))
            .children(
                s.window_insertion_apps
                    .iter()
                    .enumerate()
                    .map(|(index, bundle)| {
                        let bundle_id = bundle.clone();
                        Self::setting(
                            p,
                            tr("ui.dictation.window_insertion"),
                            bundle,
                            Button::new(("forget-window", index))
                                .small()
                                .label(tr("ui.dictation.ask_again"))
                                .on_click(cx.listener(move |this, _, _, _| {
                                    this.engine
                                        .borrow_mut()
                                        .dispatch(Command::ForgetWindowApp(bundle_id.clone()));
                                })),
                        )
                    }),
            )
            .child(self.speech_model(p, cx))
            .child(Self::setting_row(
                p,
                tr("ui.dictation.microphone"),
                Some(
                    row()
                        .gap_2()
                        .child(text(tr("ui.microphone.level"), 12., p.muted))
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
                        .unwrap_or_else(|| tr("ui.microphone.default").into()),
                    devices,
                    s.microphone.clone().unwrap_or_default(),
                    |id| Command::SetMicrophone((!id.is_empty()).then_some(id)),
                    p,
                ),
            ))
            .child(Self::setting(
                p,
                tr("ui.dictation.noise"),
                tr("ui.dictation.noise_hint"),
                gpui_component::switch::Switch::new("noise-suppression")
                    .small()
                    .checked(s.noise_suppression)
                    .on_click(cx.listener(|this, enabled: &bool, _, _| {
                        this.send(Command::SetNoiseSuppression(*enabled));
                    })),
            ))
            .child(Self::setting(
                p,
                tr("ui.dictation.spoken_language"),
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
            .child(Self::setting(
                p,
                tr("ui.language.interface"),
                "",
                self.picker(
                    "interface-language",
                    interface_name(s.interface_language).to_owned(),
                    [
                        InterfaceLanguage::System,
                        InterfaceLanguage::English,
                        InterfaceLanguage::Russian,
                    ]
                    .map(|language| (interface_name(language).to_owned(), language))
                    .into(),
                    s.interface_language,
                    Command::SetInterfaceLanguage,
                    p,
                ),
            ))
            .child(
                row()
                    .mt(px(14.))
                    .mb_1()
                    .justify_between()
                    .child(
                        text(tr("ui.dictation.permissions"), 12., p.ink)
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        button("open-introduction", tr("ui.intro.reopen"), p)
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.end_capture(cx);
                                window.focus(&this.focus);
                                this.intro = Some(onboarding::Step::Welcome);
                                cx.notify();
                            })),
                    ),
            )
            .child(Self::setting(
                p,
                tr("ui.dictation.microphone"),
                tr("ui.microphone.purpose"),
                self.permission(s.mic_allowed, true, p, cx),
            ))
            .child(Self::setting(
                p,
                tr("ui.permission.accessibility"),
                tr("ui.accessibility.purpose"),
                self.permission(s.access_allowed, false, p, cx),
            ))
            .child(group_title(tr("ui.dictation.try"), p))
            .child(
                div()
                    .relative()
                    .w_full()
                    .child(
                        canvas(
                            {
                                let frame = self.test_field_frame.clone();
                                move |bounds, window, cx| {
                                    frame.set(window.display(cx).map(|d| {
                                        use objc2_core_foundation::{CGPoint, CGRect, CGSize};
                                        let screen =
                                            objc2_core_graphics::CGDisplayBounds(d.id().into());
                                        let pos = window.bounds().origin + bounds.origin;
                                        CGRect::new(
                                            CGPoint::new(
                                                screen.origin.x + f64::from(pos.x),
                                                screen.origin.y + f64::from(pos.y),
                                            ),
                                            CGSize::new(
                                                f64::from(bounds.size.width),
                                                f64::from(bounds.size.height),
                                            ),
                                        )
                                    }));
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(Input::new(&self.test).h(px(58.)).w_full()),
            )
            .when(crate::ui::updates::available(), |page| {
                page.child(group_title(tr("ui.updates.title"), p))
                    .child(Self::setting(
                        p,
                        tr("ui.updates.automatic"),
                        concat!("Voai ", env!("CARGO_PKG_VERSION")),
                        gpui_component::switch::Switch::new("automatic-updates")
                            .small()
                            .checked(crate::ui::updates::automatic())
                            .on_click(cx.listener(|_, enabled: &bool, _, cx| {
                                crate::ui::updates::set_automatic(*enabled);
                                cx.notify();
                            })),
                    ))
            })
    }
}
